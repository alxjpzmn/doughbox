import { useState } from "react";
import { Input } from "@/components/Input";
import useAuth from "@/hooks/useAuth";


const Login = () => {
  const { login } = useAuth();
  const [password, setPassword] = useState("")


  return (
    <div className="h-svh flex items-center">
      <form className="w-full" onSubmit={(event) => { event.preventDefault(); login(password) }}
      >
        <Input
          className="mx-auto max-w-xs"
          placeholder="Enter password"
          onChange={(event) => setPassword(event.target.value)}
          autoFocus
          type="password" />
      </form>
    </div>
  )
}

export default Login;
