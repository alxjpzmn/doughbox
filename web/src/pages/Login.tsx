import { Badge } from "@/components/Badge";
import { Input } from "@/components/Input";
import useAuth from "@/hooks/useAuth";
import { useForm } from "react-hook-form";
import { yupResolver } from '@hookform/resolvers/yup';
import * as yup from "yup";

const schema = yup.object({
  password: yup.string().required('Password is required'),
}).required();
type FormData = yup.InferType<typeof schema>;


const Login = () => {
  const { login } = useAuth();
  const { register, handleSubmit, formState: { errors }, setError } = useForm<FormData>({ resolver: yupResolver(schema) });

  const onSubmit = async (data: FormData) => {
    try {
      await login(data.password);
    } catch (err) {
      setError("password", { type: "server", message: 'Login failed' });
    }
  };


  return (
    <div className="h-svh flex items-center">
      <form className="w-full" onSubmit={handleSubmit(onSubmit)}
      >
        <div
          className="mx-auto max-w-xs relative"
        >
          <Input
            {...register("password", { required: true })}
            placeholder="Enter password"
            autoFocus
            hasError={!!errors.password}
            aria-invalid={errors.password ? "true" : "false"}
            type="password" />
          {errors.password && <Badge variant="error" className="absolute t-0 l-0 mt-2">{errors.password?.message}</Badge>}
        </div>
      </form>
    </div>
  )
}

export default Login;
